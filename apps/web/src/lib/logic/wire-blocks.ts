/**
 * Wire shapes for identifiers, blocks, cards, filters, views, and typed block
 * normalizers, re-exported through `wire.ts`.
 */
import { encodeBase32 } from './base32.ts';
import { diagnostics } from './diagnostics.ts';
import { readRecurrence, type Recurrence } from './recurrence.ts';
import type { RelationLink } from './relations.ts';
import { optionalViewFields, type OptionalViewFields } from './view-members.ts';
import type { StoredDateValue } from './wire.ts';

// ------------------------------------------------------------ identifiers

export type IdKind =
  'none' | 'workspace' | 'board' | 'card' | 'view' | 'session' | 'user' | 'token' | 'block';

const ID_PREFIX: Record<IdKind, string> = {
  none: '7',
  workspace: 'w',
  board: 'b',
  card: 'c',
  view: 'v',
  session: 's',
  user: 'u',
  token: 'k',
  block: 'a',
};

/** A new identifier: one letter for the kind and 26 characters of 128 random bits. */
export function createId(kind: IdKind): string {
  const bytes = new Uint8Array(16);
  crypto.getRandomValues(bytes);
  return ID_PREFIX[kind] + encodeBase32(bytes);
}

/** The identifier kind for a block of the given type. */
export function idKindOfBlockType(type: string | undefined): IdKind {
  switch (type) {
    case 'workspace':
    case 'board':
    case 'card':
    case 'view':
      return type;
    default:
      return 'none';
  }
}

// ----------------------------------------------------------------- blocks

export const CONTENT_BLOCK_TYPES = [
  'text',
  'image',
  'divider',
  'checkbox',
  'h1',
  'h2',
  'h3',
  'list-item',
  'attachment',
  'quote',
  'video',
] as const;

export const BLOCK_TYPES = [
  ...CONTENT_BLOCK_TYPES,
  'board',
  'view',
  'card',
  'comment',
  'page',
  'entry',
  'unit',
  'chapter',
  'unknown',
] as const;

export type ContentBlockType = (typeof CONTENT_BLOCK_TYPES)[number];
export type BlockType = (typeof BLOCK_TYPES)[number];

export interface Block {
  id: string;
  containerId: string;
  parentId: string;
  createdBy: string;
  modifiedBy: string;
  type: BlockType;
  title: string;
  fields: Record<string, unknown>;
  createdAt: number;
  updatedAt: number;
  deletedAt?: number;
  limited?: boolean;
  number?: number;
}

export interface BlockPatch {
  parentId?: string;
  type?: BlockType;
  title?: string;
  updatedFields?: Record<string, unknown>;
  deletedFields?: string[];
  readAt?: number;
}

export function createBlock(source?: Block): Block {
  const now = Date.now();
  return {
    id: source?.id || createId(idKindOfBlockType(source?.type)),
    containerId: source?.containerId || '',
    parentId: source?.parentId || '',
    createdBy: source?.createdBy || '',
    modifiedBy: source?.modifiedBy || '',
    type: source?.type || 'unknown',
    fields: source?.fields ? { ...source.fields } : {},
    title: source?.title || '',
    createdAt: source?.createdAt || now,
    updatedAt: source?.updatedAt || now,
    ...(source?.deletedAt === undefined ? {} : { deletedAt: source.deletedAt }),
    limited: Boolean(source?.limited),
    ...(source?.number === undefined ? {} : { number: source.number }),
  };
}

// ------------------------------------------------------------------ cards

export type PropertyValue = string | string[] | StoredDateValue | RelationLink[];
export type ContentOrder = (string | string[])[];

export type CardFields = {
  icon?: string;
  isTemplate?: boolean;
  properties: Record<string, PropertyValue>;
  contentOrder: ContentOrder;
  /** How the card repeats (`logic/recurrence.ts`); none for one that does not. */
  recurrence?: Recurrence;
  /** Of a card done that repeated: the card the server made after it. */
  recurredTo?: string;
};

export type Card = Block & { fields: CardFields; lastCommentAt?: number };

function readContentOrder(value: unknown): ContentOrder {
  if (!Array.isArray(value)) return [];
  const order: ContentOrder = [];
  for (const entry of value as unknown[]) {
    if (typeof entry === 'string') order.push(entry);
    else if (Array.isArray(entry)) order.push([...(entry as string[])]);
  }
  return order;
}

export function createCard(source?: Block): Card {
  const fields = source?.fields ?? {};
  const isTemplate = Boolean(fields.isTemplate);
  const base = createBlock(source ? { ...source, type: 'card' } : ({ type: 'card' } as Block));
  if (isTemplate) delete base.number;
  const recurrence = readRecurrence(fields.recurrence);
  const recurredTo = typeof fields.recurredTo === 'string' ? fields.recurredTo : undefined;
  return {
    ...base,
    type: 'card',
    fields: {
      icon: (fields.icon as string | undefined) || '',
      properties: { ...(fields.properties as Record<string, PropertyValue> | undefined) },
      contentOrder: readContentOrder(fields.contentOrder),
      isTemplate,
      ...(recurrence ? { recurrence } : {}),
      ...(recurredTo ? { recurredTo } : {}),
    },
  };
}

/** How the card repeats; undefined for one that does not. */
export const cardRecurrence = (card: Card): Recurrence | undefined => card.fields.recurrence;
/** The card made after this one, done, when it repeated. */
export const cardRecurredTo = (card: Card): string | undefined => card.fields.recurredTo;
/** The card repeating as `recurrence` says, or not at all. */
export function withCardRecurrence<T extends Card>(card: T, recurrence: Recurrence | undefined): T {
  const { recurrence: _old, ...fields } = card.fields;
  return { ...card, fields: recurrence ? { ...fields, recurrence } : fields };
}

export function cardProperty(card: Card, propertyId: string): PropertyValue | undefined {
  return card.fields.properties[propertyId];
}
export function cardProperties(card: Card): Readonly<Record<string, PropertyValue>> {
  return card.fields.properties;
}
export function setCardProperty(card: Card, propertyId: string, value: PropertyValue): void {
  card.fields.properties[propertyId] = value;
}
export const isCardTemplate = (card: Card): boolean => Boolean(card.fields.isTemplate);
export function cardContentOrder(card: Card): ContentOrder {
  return card.fields.contentOrder;
}
export function cardIcon(card: Card): string {
  return card.fields.icon ?? '';
}
export function appendCardContent(card: Card, blockId: string): void {
  card.fields.contentOrder.push(blockId);
}

// ---------------------------------------------------------------- filters

export type FilterCondition =
  | 'includes'
  | 'notIncludes'
  | 'isEmpty'
  | 'isNotEmpty'
  | 'isSet'
  | 'isNotSet'
  | 'is'
  | 'contains'
  | 'notContains'
  | 'startsWith'
  | 'notStartsWith'
  | 'endsWith'
  | 'notEndsWith'
  | 'isBefore'
  | 'isAfter'
  | 'isWithin';

export type FilterClause = {
  propertyId: string;
  condition: FilterCondition;
  values: string[];
};

export type FilterGroupOperation = 'and' | 'or';

export type FilterGroup = {
  operation: FilterGroupOperation;
  filters: (FilterClause | FilterGroup)[];
};

export function isFilterGroup(filter: FilterClause | FilterGroup): filter is FilterGroup {
  return 'operation' in filter && 'filters' in filter;
}

export function createFilterClause(source?: Partial<FilterClause>): FilterClause {
  return {
    propertyId: source?.propertyId || '',
    condition: source?.condition || 'includes',
    values: Array.isArray(source?.values) ? source.values.slice() : [],
  };
}

export function createFilterGroup(source?: FilterGroup): FilterGroup {
  return {
    operation: source?.operation || 'and',
    filters: (source?.filters ?? []).map((filter) =>
      isFilterGroup(filter) ? createFilterGroup(filter) : createFilterClause(filter),
    ),
  };
}

// ------------------------------------------------------------------ views

export type ViewType = 'board' | 'table' | 'gallery' | 'calendar' | 'gantt';
export const TITLE_COLUMN_ID = '__title';
export type SortOption = { propertyId: string; reversed: boolean };
export type GroupCalculation = { calculation: string; propertyId: string };

export type BoardViewFields = {
  viewType: ViewType;
  sort: SortOption[];
  shownProperties: string[];
  visibleGroups: string[];
  hiddenGroups: string[];
  collapsedGroups: string[];
  filter: FilterGroup;
  cardOrder: string[];
  columnWidths: Record<string, number>;
  columnCalculations: Record<string, string>;
  groupCalculations: Record<string, GroupCalculation>;
  defaultTemplateId: string;
} & OptionalViewFields;

export type BoardView = Block & { fields: BoardViewFields };

const copyList = (value: unknown): string[] =>
  Array.isArray(value) ? [...(value as string[])] : [];

export function createBoardView(source?: Block): BoardView {
  const fields = (source?.fields ?? {}) as Partial<BoardViewFields>;
  return {
    ...createBlock(source ? { ...source, type: 'view' } : ({ type: 'view' } as Block)),
    type: 'view',
    fields: {
      viewType: fields.viewType || 'board',
      ...optionalViewFields(fields),
      sort: (fields.sort ?? []).map((option) => ({ ...option })),
      shownProperties: copyList(fields.shownProperties),
      visibleGroups: copyList(fields.visibleGroups),
      hiddenGroups: copyList(fields.hiddenGroups),
      collapsedGroups: copyList(fields.collapsedGroups),
      filter: createFilterGroup(fields.filter),
      cardOrder: copyList(fields.cardOrder),
      columnWidths: { ...fields.columnWidths },
      columnCalculations: { ...fields.columnCalculations },
      groupCalculations: { ...fields.groupCalculations },
      defaultTemplateId: fields.defaultTemplateId || '',
    },
  };
}

export interface ViewQuery {
  viewType: ViewType;
  filter: FilterGroup;
  sort: readonly SortOption[];
  cardOrder: readonly string[];
  shownProperties: readonly string[];
  dateProperty: string | undefined;
}

export function viewQuery(view: BoardView): ViewQuery {
  const { fields } = view;
  return {
    viewType: fields.viewType,
    filter: fields.filter,
    sort: fields.sort,
    cardOrder: fields.cardOrder,
    shownProperties: fields.shownProperties,
    dateProperty: fields.dateProperty,
  };
}

export interface ViewGrouping {
  groupBy: string | undefined;
  visibleGroups: readonly string[];
  hiddenGroups: readonly string[];
  collapsedGroups: readonly string[];
}

export function viewGrouping(view: BoardView): ViewGrouping {
  const { fields } = view;
  return {
    groupBy: fields.groupBy,
    visibleGroups: fields.visibleGroups,
    hiddenGroups: fields.hiddenGroups,
    collapsedGroups: fields.collapsedGroups,
  };
}

// ----------------------------------------------------- other block kinds

export type CommentBlock = Block & { type: 'comment' };
export type TextBlock = Block & { type: 'text' };
export type HeadingBlock = Block & { type: 'h1' | 'h2' | 'h3' };
export type DividerBlock = Block & { type: 'divider' };
export type CheckboxBlock = Block & { type: 'checkbox' };
export type ImageBlock = Block & { type: 'image'; fields: { fileId: string } };
export type AttachmentBlock = Block & { type: 'attachment'; fields: { fileId: string } };

function createTyped<T extends BlockType>(type: T, source?: Block): Block & { type: T } {
  return { ...createBlock(source), type };
}

export const createCommentBlock = (source?: Block): CommentBlock => createTyped('comment', source);
export const createTextBlock = (source?: Block): TextBlock => createTyped('text', source);
export const createDividerBlock = (source?: Block): DividerBlock => createTyped('divider', source);
export const createCheckboxBlock = (source?: Block): CheckboxBlock =>
  createTyped('checkbox', source);

export function createHeadingBlock(level: 1 | 2 | 3, source?: Block): HeadingBlock {
  return createTyped(`h${level}`, source);
}

export function createImageBlock(source?: Block): ImageBlock {
  return {
    ...createTyped('image', source),
    fields: { fileId: (source?.fields.fileId as string | undefined) || '' },
  };
}

export function createAttachmentBlock(source?: Block): AttachmentBlock {
  const fields = source?.fields ?? {};
  return {
    ...createTyped('attachment', source),
    fields: {
      fileId:
        (fields.attachmentId as string | undefined) || (fields.fileId as string | undefined) || '',
    },
  };
}

export function blockFileId(block: ImageBlock | AttachmentBlock): string {
  return block.fields.fileId;
}

export function normalizeBlock(block: Block): Block {
  switch (block.type) {
    case 'view':
      return createBoardView(block);
    case 'card':
      return createCard(block);
    case 'text':
      return createTextBlock(block);
    case 'h1':
      return createHeadingBlock(1, block);
    case 'h2':
      return createHeadingBlock(2, block);
    case 'h3':
      return createHeadingBlock(3, block);
    case 'image':
      return createImageBlock(block);
    case 'divider':
      return createDividerBlock(block);
    case 'comment':
      return createCommentBlock(block);
    case 'checkbox':
      return createCheckboxBlock(block);
    case 'attachment':
      return createAttachmentBlock(block);
    default:
      diagnostics.report(`cannot normalize a block of unknown type: ${block.type}`);
      return createBlock(block);
  }
}

export function normalizeBlocks(blocks: readonly Block[]): Block[] {
  return blocks.map(normalizeBlock);
}

export function remapBlockReferences(block: Block, idMap: Readonly<Record<string, string>>): void {
  const mapped = (id: string): string => idMap[id] ?? id;
  if (block.type === 'view') {
    const view = block as BoardView;
    view.fields.cardOrder = view.fields.cardOrder.map(mapped);
  }
  if (block.type === 'card') {
    const card = block as Card;
    card.fields.contentOrder = card.fields.contentOrder.map((entry) =>
      Array.isArray(entry) ? entry.map(mapped) : mapped(entry),
    );
  }
}

/**
 * The member of a comment's `fields` that names the personal API token an
 * agent wrote it through: the server sets it, and the comment shows its
 * author's agent by that name.
 */
export const COMMENT_AGENT_TOKEN_NAME = 'agentTokenName';

/** The name of the token an agent wrote `comment` through; `undefined` for a person's. */
export function agentTokenNameOf(comment: Pick<Block, 'fields'>): string | undefined {
  const name = comment.fields[COMMENT_AGENT_TOKEN_NAME];
  return typeof name === 'string' && name !== '' ? name : undefined;
}
